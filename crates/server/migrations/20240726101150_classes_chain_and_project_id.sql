ALTER TABLE contract_classes
ADD COLUMN chain_id VARCHAR(66) DEFAULT NULL,
ADD COLUMN project_id INTEGER DEFAULT NULL,
ADD CONSTRAINT fk_project
    FOREIGN KEY (project_id) 
    REFERENCES projects(id);